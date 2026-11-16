//! Informational pages own publication, language content and the complete slug history.
use sqlx::{FromRow, PgPool};
use talocart_platform::{
    content::{ContentError, Definition, Draft, Language, Translation},
    identity::{Identity, IdentityError, Permission, Session},
};
use thiserror::Error;

pub const CONTENT: Definition = Definition {
    table: "information_pages",
    name_column: "name",
    description_column: Some("description"),
    name_limit: 120,
    description_limit: 20_000,
    seo: true,
    read: Permission::PagesRead,
    write: Permission::PagesWrite,
};
pub const MAX_PAGES: i64 = 200;
pub const MAX_MENU_LINKS: i64 = 20;
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub seo_title: String,
    pub seo_description: String,
    pub published: bool,
    pub header_menu: bool,
    pub footer_menu: bool,
    pub sort_order: String,
}
#[derive(Clone, Debug, FromRow)]
pub struct Page {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub seo_title: String,
    pub seo_description: String,
    pub published: bool,
    pub header_menu: bool,
    pub footer_menu: bool,
    pub sort_order: i32,
    pub revision: i64,
}
/// Fields rendered by the paginated administration list; excludes bodies and SEO.
#[derive(Clone, Debug, FromRow)]
pub struct PageSummary {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub published: bool,
    pub header_menu: bool,
    pub footer_menu: bool,
    pub sort_order: i32,
}
impl Page {
    pub fn input(&self) -> Input {
        Input {
            slug: self.slug.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            seo_title: self.seo_title.clone(),
            seo_description: self.seo_description.clone(),
            published: self.published,
            header_menu: self.header_menu,
            footer_menu: self.footer_menu,
            sort_order: self.sort_order.to_string(),
        }
    }
}
#[derive(Debug, Error)]
pub enum PageError {
    #[error(transparent)]
    Technical(#[from] talocart_platform::technical::TechnicalError),
    #[error(transparent)]
    Access(#[from] talocart_platform::technical::AccessError),
    #[error("Invalid page fields")]
    Invalid(Vec<&'static str>),
    #[error("Page not found")]
    NotFound,
    #[error("Page changed")]
    Stale,
    #[error("Slug reserved")]
    Duplicate,
    #[error("Page or menu limit reached")]
    Limit,
    #[error(transparent)]
    Content(#[from] ContentError),
    #[error(transparent)]
    Identity(IdentityError),
}
impl From<sqlx::Error> for PageError {
    fn from(e: sqlx::Error) -> Self {
        if e.as_database_error()
            .is_some_and(|e| e.is_unique_violation())
        {
            Self::Duplicate
        } else {
            Self::Technical(
                talocart_platform::technical::DatabaseFailure::capture(e, module_path!()).into(),
            )
        }
    }
}
type Result<T> = std::result::Result<T, PageError>;
pub struct Pages {
    pool: PgPool,
    identity: Identity,
}
impl Pages {
    pub fn new(pool: PgPool, identity: Identity) -> Self {
        Self { pool, identity }
    }
    fn projection(language: Language) -> String {
        let fields = ["name", "description", "seo_title", "seo_description"]
            .map(|field| {
                format!(
                    "{} AS {field}",
                    language.expression(field, &format!("p.{field}"))
                )
            })
            .join(",");
        format!(
            "SELECT p.id,p.slug,{fields},p.published,p.header_menu,p.footer_menu,p.sort_order,\
                p.revision \
                FROM information_pages p {}",
            language.joins("information_pages", "p.id")
        )
    }
    pub async fn list(
        &self,
        s: &Session,
        page: i64,
        language: Language,
    ) -> Result<talocart_platform::pagination::Page<PageSummary>> {
        self.identity.authorize(s, Permission::PagesRead).await?;
        if !(1..=1_000_000).contains(&page) {
            return Err(PageError::Invalid(vec!["page"]));
        }
        Ok(talocart_platform::pagination::read(
            &self.pool,
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT p.id,p.slug,{} AS name,p.published,p.header_menu,p.footer_menu,p.sort_order \
                    FROM information_pages p {}",
                language.expression("name", "p.name"),
                language.joins("information_pages", "p.id"),
            ))),
            "sort_order,id",
            page,
            20,
        )
        .await?)
    }
    pub async fn get(&self, s: &Session, id: i64, language: Language) -> Result<Page> {
        self.identity.authorize(s, Permission::PagesRead).await?;
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{} WHERE p.id=$1",
            Self::projection(language)
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(PageError::NotFound)
    }
    pub async fn public(&self, slug: &str, language: Language) -> Result<Page> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{} JOIN information_page_slugs a ON a.page_id=p.id WHERE a.slug=$1 AND p.published",
            Self::projection(language)
        )))
        .bind(slug)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(PageError::NotFound)
    }
    pub async fn navigation(&self, language: Language) -> Result<Vec<PageLink>> {
        let name = language.expression("name", "p.name");
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT p.slug,{name} AS name,p.header_menu,p.footer_menu \
                FROM information_pages p {} \
                WHERE p.published \
                AND (p.header_menu \
                    OR p.footer_menu) \
                ORDER BY p.sort_order,p.id \
                LIMIT $1",
            language.joins("information_pages", "p.id")
        )))
        .bind(MAX_PAGES)
        .fetch_all(&self.pool)
        .await?)
    }
    pub async fn sitemap(&self) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT slug FROM information_pages WHERE published ORDER BY id LIMIT $1",
        )
        .bind(MAX_PAGES)
        .fetch_all(&self.pool)
        .await?)
    }
    /// Authorization, language policy, revision and menu capacity commit with all translations.
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
        s.require(Permission::PagesWrite)?;
        let order = validate(input)?;
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::PagesWrite).await?;
        // prepare acquires the shared language/commerce lock before reading content.
        let rows = CONTENT
            .prepare(
                &mut tx,
                draft,
                id,
                Translation {
                    locale: String::new(),
                    name: input.name.clone(),
                    description: input.description.clone(),
                    seo_title: input.seo_title.clone(),
                    seo_description: input.seo_description.clone(),
                },
            )
            .await?;
        let primary: String =
            sqlx::query_scalar("SELECT primary_locale FROM storefront_settings WHERE id=1")
                .fetch_one(&mut *tx)
                .await?;
        let main = rows
            .iter()
            .find(|r| r.locale == primary)
            .ok_or(PageError::Invalid(vec!["name"]))?;
        if input.published && main.description.is_empty() {
            return Err(PageError::Invalid(vec!["description"]));
        }
        if id == 0 && revision != 0 {
            return Err(PageError::Stale);
        }
        if id == 0 {
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM information_pages")
                .fetch_one(&mut *tx)
                .await?;
            if count >= MAX_PAGES {
                return Err(PageError::Limit);
            }
        } else {
            let current: Option<i64> =
                sqlx::query_scalar("SELECT revision FROM information_pages WHERE id=$1 FOR UPDATE")
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await?;
            if current != Some(revision) {
                return Err(PageError::Stale);
            }
        }
        if input.published {
            let (header, footer): (i64, i64) = sqlx::query_as(
                "SELECT count(*) FILTER(WHERE header_menu),count(*) FILTER(WHERE footer_menu) \
                FROM information_pages \
                WHERE published \
                AND id<>$1",
            )
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
            if (input.header_menu && header >= MAX_MENU_LINKS)
                || (input.footer_menu && footer >= MAX_MENU_LINKS)
            {
                return Err(PageError::Limit);
            }
        }
        let sql = if id == 0 {
            "INSERT INTO information_pages(slug,name,description,seo_title,seo_description,published,\
                    header_menu,footer_menu,sort_order) \
                SELECT $1,$2,$3,$4,$5,$6,$7,$8,$9 \
                WHERE $10=0 \
                RETURNING id"
        } else {
            "UPDATE information_pages \
                SET slug=$1,name=$2,description=$3,seo_title=$4,seo_description=$5,published=$6,header_menu=$7,\
                footer_menu=$8,sort_order=$9,revision=revision+1 \
                WHERE id=$10 \
                RETURNING id"
        };
        let id: i64 = sqlx::query_scalar(sql)
            .bind(input.slug.trim())
            .bind(&main.name)
            .bind(&main.description)
            .bind(&main.seo_title)
            .bind(&main.seo_description)
            .bind(input.published)
            .bind(input.header_menu)
            .bind(input.footer_menu)
            .bind(order)
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        CONTENT.persist(&mut tx, id, &rows).await?;
        tx.commit().await?;
        Ok(id)
    }
    pub async fn delete(&self, s: &Session, csrf: &str, id: i64, revision: i64) -> Result<()> {
        s.check_csrf(csrf)?;
        s.require(Permission::PagesWrite)?;
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::PagesWrite).await?;
        talocart_platform::database::commercial_lock::exclusive(&mut tx).await?;
        if sqlx::query("DELETE FROM information_pages WHERE id=$1 AND revision=$2")
            .bind(id)
            .bind(revision)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            != 1
        {
            return Err(PageError::Stale);
        }
        tx.commit().await?;
        Ok(())
    }
}
#[derive(FromRow)]
pub struct PageLink {
    pub slug: String,
    pub name: String,
    pub header_menu: bool,
    pub footer_menu: bool,
}
fn validate(input: &Input) -> Result<i32> {
    let mut fields = vec![];
    if !crate::valid_slug(input.slug.trim()) {
        fields.push("slug");
    }
    let order = input
        .sort_order
        .trim()
        .parse::<i32>()
        .ok()
        .filter(|n| (-999999..=999999).contains(n));
    if order.is_none() {
        fields.push("sort_order");
    }
    if !fields.is_empty() {
        return Err(PageError::Invalid(fields));
    }
    Ok(order.unwrap_or_default())
}
impl From<IdentityError> for PageError {
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
    fn page_urls_and_menu_order_are_bounded() {
        let mut i = Input {
            slug: "about-us".into(),
            sort_order: "-999999".into(),
            ..Input::default()
        };
        assert_eq!(validate(&i).unwrap(), -999999);
        for slug in [
            "../admin",
            "a/b",
            "UPPER",
            "a--b",
            "",
            "javascript:alert(1)",
        ] {
            i.slug = slug.into();
            assert!(validate(&i).is_err());
        }
        i.slug = "about-us".into();
        i.sort_order = "1000000".into();
        assert!(validate(&i).is_err());
    }
}
