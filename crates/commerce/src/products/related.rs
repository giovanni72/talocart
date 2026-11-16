//! Directed product recommendations, included in the base catalog.
use super::*;

#[derive(FromRow)]
pub struct RelatedChoice {
    pub id: i64,
    pub name: String,
    pub model: String,
}

pub struct RelatedEditor {
    pub fallback: String,
    pub selected: Vec<RelatedChoice>,
    pub candidates: Vec<RelatedChoice>,
}

impl Products {
    pub async fn related_editor(
        &self,
        session: &Session,
        id: i64,
        search: &str,
    ) -> Result<RelatedEditor> {
        self.entry(session, id).await?;
        if search.chars().count() > 120 {
            return Err(ProductError::Invalid(vec![]));
        }
        let fallback =
            sqlx::query_scalar("SELECT fallback FROM catalog_related_settings WHERE product_id=$1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
                .unwrap_or_else(|| "none".into());
        let selected = sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            "SELECT p.id,p.name,p.model FROM [[products]] p JOIN catalog_related_products r ON r.related_id=p.id WHERE r.product_id=$1 ORDER BY r.position", self.language)))
            .bind(id).fetch_all(&self.pool).await?;
        let candidates = if search.trim().is_empty() {
            Vec::new()
        } else {
            sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            "SELECT p.id,p.name,p.model FROM [[products]] p WHERE p.id<>$1 AND (strpos(lower(p.name),lower($2))>0 OR strpos(lower(p.model),lower($2))>0) AND NOT EXISTS(SELECT 1 FROM catalog_related_products r WHERE r.product_id=$1 AND r.related_id=p.id) ORDER BY lower(p.name),p.id LIMIT 25", self.language)))
            .bind(id).bind(search.trim()).fetch_all(&self.pool).await?
        };
        Ok(RelatedEditor {
            fallback,
            selected,
            candidates,
        })
    }

    /// Serialized per source; each action changes only its own setting or link.
    pub async fn change_related(
        &self,
        session: &Session,
        csrf: &str,
        id: i64,
        action: &str,
        target: i64,
        fallback: &str,
    ) -> Result<()> {
        session.check_csrf(csrf)?;
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::ProductsWrite).await?;
        let source: Option<i64> =
            sqlx::query_scalar("SELECT id FROM catalog_products WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        if source.is_none() {
            return Err(ProductError::NotFound);
        }
        match action {
            "fallback" if ["none", "category", "brand", "either"].contains(&fallback) => {
                sqlx::query("INSERT INTO catalog_related_settings(product_id,fallback) VALUES($1,$2) ON CONFLICT(product_id) DO UPDATE SET fallback=EXCLUDED.fallback")
                    .bind(id).bind(fallback).execute(&mut *tx).await?;
            }
            "remove" if target > 0 => {
                sqlx::query(
                    "DELETE FROM catalog_related_products WHERE product_id=$1 AND related_id=$2",
                )
                .bind(id)
                .bind(target)
                .execute(&mut *tx)
                .await?;
            }
            "add" if target > 0 && target != id => {
                let exists: bool =
                    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM catalog_products WHERE id=$1)")
                        .bind(target)
                        .fetch_one(&mut *tx)
                        .await?;
                if !exists {
                    return Err(ProductError::NotFound);
                }
                let changed = sqlx::query("INSERT INTO catalog_related_products(product_id,related_id,position) SELECT $1,$2,slot::smallint FROM generate_series(1,12) slot WHERE NOT EXISTS(SELECT 1 FROM catalog_related_products WHERE product_id=$1 AND position=slot) ORDER BY slot LIMIT 1 ON CONFLICT(product_id,related_id) DO NOTHING")
                    .bind(id).bind(target).execute(&mut *tx).await?;
                if changed.rows_affected() == 0 {
                    return Err(ProductError::Invalid(vec![]));
                }
            }
            _ => return Err(ProductError::Invalid(vec![])),
        }
        tx.commit().await?;
        Ok(())
    }

    /// One bounded query uses the same visibility, translated content and prices as catalog cards.
    /// Explicit links suppress automatic suggestions even when their targets are hidden.
    pub async fn public_related(&self, id: i64) -> Result<Vec<CardEntry>> {
        let sql = format!(
            "{PUBLIC_CARD_SELECT} JOIN catalog_visible_products origin ON origin.id=$1 LEFT JOIN catalog_related_settings config ON config.product_id=origin.id LEFT JOIN catalog_related_products link ON link.product_id=origin.id AND link.related_id=p.id WHERE p.id<>origin.id AND (link.related_id IS NOT NULL OR (NOT EXISTS(SELECT 1 FROM catalog_related_products r WHERE r.product_id=origin.id) AND ((config.fallback IN ('category','either') AND p.category_id=origin.category_id) OR (config.fallback IN ('brand','either') AND p.brand_id=origin.brand_id)))) ORDER BY link.position NULLS LAST,lower(p.name),p.id LIMIT 4"
        );
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            sql,
            self.language,
        )))
        .bind(id)
        .fetch_all(&self.pool)
        .await?)
    }
}
