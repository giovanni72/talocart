//! Homepage banners belong to the mandatory base; sources and derivatives use global media.
use crate::{
    content::{ContentError, Definition, Draft, Language, Translation},
    identity::{Identity, IdentityError, Permission, Session},
    media::{MediaError, MediaStore},
};
use sqlx::{FromRow, PgPool};
use thiserror::Error;

pub const MAX_BANNERS: i64 = 50;
pub const MAX_ACTIVE: i64 = 10;
pub const CONTENT: Definition = Definition {
    table: "home_banners",
    name_column: "name",
    description_column: Some("description"),
    name_limit: 250,
    description_limit: 2048,
    seo: false,
    read: Permission::SettingsRead,
    write: Permission::SettingsWrite,
};
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub label: String,
    /// Accessible description of the banner/link in the primary content language.
    pub name: String,
    /// Optional destination; stored through the shared multilingual content contract.
    pub description: String,
    pub image_id: String,
    pub active: bool,
    pub sort_order: String,
}
#[derive(Debug, FromRow)]
pub struct Banner {
    pub id: i64,
    pub label: String,
    pub name: String,
    pub description: String,
    pub image_id: i64,
    pub width: i32,
    pub height: i32,
    pub active: bool,
    pub sort_order: i32,
    pub revision: i64,
}
impl Banner {
    pub fn input(&self) -> Input {
        Input {
            label: self.label.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            image_id: self.image_id.to_string(),
            active: self.active,
            sort_order: self.sort_order.to_string(),
        }
    }
    pub fn image(&self) -> String {
        format!("/media/image/{}/large", self.image_id)
    }
    pub fn srcset(&self) -> String {
        crate::media::responsive_srcset(Some(self.image_id), Some(self.width), Some(self.height))
    }
}
#[derive(Debug, Error)]
pub enum BannerError {
    #[error(transparent)]
    Technical(#[from] crate::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] crate::technical::AccessError),
    #[error("Invalid banner fields")]
    Invalid(Vec<&'static str>),
    #[error("Banner not found")]
    NotFound,
    #[error("Banner changed")]
    Stale,
    #[error("Banner capacity exceeded")]
    Limit,
    #[error(transparent)]
    Content(#[from] ContentError),
    #[error(transparent)]
    Identity(IdentityError),
    #[error(transparent)]
    Media(#[from] MediaError),
}
impl From<sqlx::Error> for BannerError {
    fn from(e: sqlx::Error) -> Self {
        Self::Technical(crate::technical::DatabaseFailure::capture(e, module_path!()).into())
    }
}
type Result<T> = std::result::Result<T, BannerError>;
pub struct Banners {
    pool: PgPool,
    identity: Identity,
}
impl Banners {
    pub fn new(pool: PgPool, identity: Identity) -> Self {
        Self { pool, identity }
    }
    fn projection(language: Language) -> String {
        format!(
            "SELECT b.id,b.label,{} AS name,{} AS description,m.id AS image_id,m.width,m.height,b.active,b.sort_order,b.revision FROM home_banners b JOIN media_objects m ON m.key=b.image_key JOIN media_library l ON l.key=b.image_key {}",
            language.expression("name", "b.name"),
            language.expression("description", "b.description"),
            language.joins("home_banners", "b.id")
        )
    }
    pub async fn list(
        &self,
        s: &Session,
        page: i64,
        language: Language,
    ) -> Result<crate::pagination::Page<Banner>> {
        self.identity.authorize(s, Permission::SettingsRead).await?;
        if !(1..=1_000_000).contains(&page) {
            return Err(BannerError::Invalid(vec!["page"]));
        }
        Ok(crate::pagination::read(
            &self.pool,
            sqlx::query(sqlx::AssertSqlSafe(Self::projection(language))),
            "sort_order,id",
            page,
            20,
        )
        .await?)
    }
    pub async fn get(&self, s: &Session, id: i64, language: Language) -> Result<Banner> {
        self.identity.authorize(s, Permission::SettingsRead).await?;
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{} WHERE b.id=$1",
            Self::projection(language)
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(BannerError::NotFound)
    }
    pub async fn public(&self, language: Language) -> Result<Vec<Banner>> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("{} WHERE b.active AND m.public AND l.owner_id IS NULL AND l.trashed_at IS NULL ORDER BY b.sort_order,b.id LIMIT $1", Self::projection(language))))
            .bind(MAX_ACTIVE).fetch_all(&self.pool).await?)
    }
    /// Media lock precedes identity/content locks, as in the global library/branding writes.
    /// Sources, translations, revision and cache queue commit atomically; no image CPU work here.
    pub async fn save(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        revision: i64,
        input: &Input,
        draft: Option<&Draft>,
    ) -> Result<i64> {
        s.check_csrf(csrf)?;
        s.require(Permission::SettingsWrite)?;
        let (image_id, order) = validate(input)?;
        let mut tx = self.pool.begin().await?;
        MediaStore::publish_lock(&mut tx).await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::SettingsWrite).await?;
        let rows = CONTENT
            .prepare(
                &mut tx,
                draft,
                id,
                Translation {
                    name: input.name.clone(),
                    description: input.description.clone(),
                    ..Translation::default()
                },
            )
            .await?;
        let invalid: Vec<_> = rows
            .iter()
            .filter(|r| !valid_link(&r.description))
            .map(|r| (r.locale.clone(), "description"))
            .collect();
        if !invalid.is_empty() {
            return Err(ContentError::Invalid(invalid).into());
        }
        let primary: String =
            sqlx::query_scalar("SELECT primary_locale FROM storefront_settings WHERE id=1")
                .fetch_one(&mut *tx)
                .await?;
        let main = rows
            .iter()
            .find(|r| r.locale == primary)
            .ok_or(BannerError::Invalid(vec!["name"]))?;
        let current: Option<(i64, String)> = if id > 0 {
            sqlx::query_as("SELECT revision,image_key FROM home_banners WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
        } else {
            None
        };
        if (id == 0 && revision != 0)
            || (id != 0 && current.as_ref().map(|c| c.0) != Some(revision))
        {
            return Err(BannerError::Stale);
        }
        let source = MediaStore::selected_key(&mut tx, image_id, None)
            .await
            .map_err(|e| match e {
                MediaError::Invalid | MediaError::Missing => BannerError::Invalid(vec!["image_id"]),
                e => e.into(),
            })?;
        let source_changed = current.as_ref().is_none_or(|c| c.1 != source);
        if source_changed {
            Identity::authorize_transaction(&mut tx, s, csrf, Permission::MediaRead).await?;
        }
        let (total, active): (i64, i64) = sqlx::query_as(
            "SELECT count(*),count(*) FILTER(WHERE active AND id<>$1) FROM home_banners",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if (id == 0 && total >= MAX_BANNERS) || (input.active && active >= MAX_ACTIVE) {
            return Err(BannerError::Limit);
        }
        let query = if id == 0 {
            "INSERT INTO home_banners(label,name,description,image_key,active,sort_order) SELECT $1,$2,$3,$4,$5,$6 WHERE $7=0 RETURNING id"
        } else {
            "UPDATE home_banners SET label=$1,name=$2,description=$3,image_key=$4,active=$5,sort_order=$6,revision=revision+1 WHERE id=$7 RETURNING id"
        };
        let id = sqlx::query_scalar(query)
            .bind(input.label.trim())
            .bind(&main.name)
            .bind(&main.description)
            .bind(&source)
            .bind(input.active)
            .bind(order)
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        CONTENT.persist(&mut tx, id, &rows).await?;
        if source_changed {
            MediaStore::queue_selected_source(&mut tx, &source).await?;
        }
        tx.commit().await?;
        Ok(id)
    }
    pub async fn delete(&self, s: &Session, csrf: &str, id: i64, revision: i64) -> Result<()> {
        s.check_csrf(csrf)?;
        s.require(Permission::SettingsWrite)?;
        let mut tx = self.pool.begin().await?;
        MediaStore::publish_lock(&mut tx).await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::SettingsWrite).await?;
        crate::database::commercial_lock::exclusive(&mut tx).await?;
        if sqlx::query("DELETE FROM home_banners WHERE id=$1 AND revision=$2")
            .bind(id)
            .bind(revision)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            != 1
        {
            return Err(BannerError::Stale);
        }
        tx.commit().await?;
        Ok(())
    }
}
fn validate(input: &Input) -> Result<(i64, i32)> {
    let mut errors = vec![];
    let label = input.label.trim();
    if label.is_empty() || label.chars().count() > 120 || label.chars().any(char::is_control) {
        errors.push("label");
    }
    let id = input.image_id.parse::<i64>().ok().filter(|n| *n > 0);
    if id.is_none() {
        errors.push("image_id");
    }
    let order = input
        .sort_order
        .parse::<i32>()
        .ok()
        .filter(|n| (0..=9999).contains(n));
    if order.is_none() {
        errors.push("sort_order");
    }
    if !errors.is_empty() {
        return Err(BannerError::Invalid(errors));
    }
    Ok((id.unwrap_or_default(), order.unwrap_or_default()))
}
/// Root-relative store destinations or HTTPS URLs, never network paths, credentials or scripts.
pub fn valid_link(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    if value
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
        || value.len() > 2048
    {
        return false;
    }
    if value.starts_with('/') {
        return !value.starts_with("//");
    }
    value.starts_with("https://")
        && url::Url::parse(value).is_ok_and(|u| {
            u.scheme() == "https"
                && u.host_str().is_some()
                && u.username().is_empty()
                && u.password().is_none()
        })
}
impl From<IdentityError> for BannerError {
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
    fn links_allow_only_local_paths_and_https_without_credentials() {
        for link in [
            "",
            "/catalog",
            "/products/test?q=1#info",
            "https://example.test/p?q=1",
        ] {
            assert!(valid_link(link), "{link}");
        }
        for link in [
            "//evil.test",
            "/\\evil.test",
            "javascript:alert(1)",
            "data:text/html,x",
            "http://example.test",
            "https://user:pass@example.test",
            "/test\n",
            " https://example.test",
            "https:foo",
        ] {
            assert!(!valid_link(link), "{link}");
        }
    }
    #[test]
    fn fields_are_bounded_and_images_required() {
        let mut i = Input {
            label: "Sale".into(),
            image_id: "1".into(),
            sort_order: "0".into(),
            ..Input::default()
        };
        assert_eq!(validate(&i).unwrap(), (1, 0));
        for image in ["", "0", "-1", "9223372036854775808"] {
            i.image_id = image.into();
            assert!(validate(&i).is_err());
        }
        i.image_id = "1".into();
        i.sort_order = "10000".into();
        assert!(validate(&i).is_err());
    }
}
