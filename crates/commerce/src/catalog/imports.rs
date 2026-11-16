//! Catalog-owned intake queries. Source receipts and transaction completion stay with the caller.
use crate::{
    content::Resource,
    imports::{Brand, Category, Error, Issue, Result, Writer},
};
use sqlx::{Postgres, Transaction};
use talocart_platform::identity::Permission;

pub async fn lock_import_tree(tx: &mut Transaction<'_, Postgres>) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(5426645964561798228)")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// A source label must identify at most one local brand across supported languages.
pub async fn find_import_brand(
    pool: impl sqlx::Executor<'_, Database = Postgres>,
    name: &str,
) -> Result<Option<i64>> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT entity_id \
        FROM catalog_brands_translations \
        WHERE locale IN ('it','en') \
        AND lower(name)=lower($1) \
        ORDER BY entity_id \
        LIMIT 2",
    )
    .bind(name)
    .fetch_all(pool)
    .await?;
    match ids.as_slice() {
        [] => Ok(None),
        [id] => Ok(Some(*id)),
        _ => Err(Error::Conflict),
    }
}
impl Writer<'_, '_> {
    pub async fn create_brand(&mut self, input: &Brand) -> Result<(i64, bool)> {
        self.require(Permission::BrandsWrite)?;
        let values = super::Values {
            name: input.name.clone(),
            slug: input.slug.clone(),
            active: true,
            sort_order: "0".into(),
            ..Default::default()
        };
        super::check_import_values(&values, super::Kind::Brand)
            .map_err(|_| Error::Invalid(Issue::Brand))?;
        if let Some(id) = find_import_brand(&mut **self.tx, &input.name).await? {
            return Ok((id, false));
        }
        let id=sqlx::query_scalar("INSERT INTO catalog_brands(name,slug,active,sort_order) VALUES($1,$2,true,0) RETURNING id")
            .bind(&input.name).bind(&input.slug).fetch_one(&mut **self.tx).await?;
        crate::imports::persist(self.tx, Resource::Brand, id, &input.translations).await?;
        Ok((id, true))
    }
    pub async fn create_category(&mut self, input: &Category) -> Result<i64> {
        self.require(Permission::CategoriesWrite)?;
        let values = super::Values {
            name: input.name.clone(),
            slug: input.slug.clone(),
            description: input.description.clone(),
            parent_id: input.parent_id,
            sort_order: input.sort_order.to_string(),
            active: input.active,
        };
        super::check_import_values(&values, super::Kind::Category)
            .map_err(|_| Error::Invalid(Issue::Category))?;
        lock_import_tree(self.tx).await?;
        let id = sqlx::query_scalar(
            "INSERT INTO catalog_categories(name,slug,description,parent_id,sort_order,active) \
            VALUES($1,$2,$3,$4,$5,$6) \
            RETURNING id",
        )
        .bind(&input.name)
        .bind(&input.slug)
        .bind(&input.description)
        .bind(input.parent_id)
        .bind(input.sort_order)
        .bind(input.active)
        .fetch_one(&mut **self.tx)
        .await
        .map_err(|_| Error::Invalid(Issue::Category))?;
        crate::imports::persist(self.tx, Resource::Category, id, &input.translations).await?;
        Ok(id)
    }
    /// The row lock spans publication of an already prepared local image.
    pub async fn brand_logo_for_update(&mut self, id: i64) -> Result<Option<String>> {
        self.require(Permission::BrandsWrite)?;
        Ok(
            sqlx::query_scalar("SELECT logo_key FROM catalog_brands WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_one(&mut **self.tx)
                .await?,
        )
    }
    pub async fn set_brand_logo(&mut self, id: i64, key: &str) -> Result<()> {
        self.require(Permission::BrandsWrite)?;
        sqlx::query("UPDATE catalog_brands SET logo_key=$1 WHERE id=$2")
            .bind(key)
            .bind(id)
            .execute(&mut **self.tx)
            .await?;
        Ok(())
    }
}
