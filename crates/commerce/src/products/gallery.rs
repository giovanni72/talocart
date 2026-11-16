//! A bounded gallery belongs to a product, independently of its primary/variant photo.
use super::*;
use sqlx::{Postgres, Transaction};

pub const MAX_GALLERY_IMAGES: usize = 12;
#[derive(Clone, FromRow)]
pub struct GalleryImage {
    pub id: i64,
    pub image_id: i64,
    pub image_width: i32,
    pub image_height: i32,
    pub image_key: String,
    pub position: i32,
}
impl GalleryImage {
    pub fn image_url(&self, profile: &str) -> String {
        format!("/media/image/{}/{profile}", self.image_id)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GalleryAction {
    Earlier,
    Later,
    Primary,
    Remove,
}
impl GalleryAction {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "earlier" => Ok(Self::Earlier),
            "later" => Ok(Self::Later),
            "primary" => Ok(Self::Primary),
            "remove" => Ok(Self::Remove),
            _ => Err(ProductError::GalleryAction),
        }
    }
}
const GALLERY_SELECT: &str = "SELECT g.id,g.position,g.image_key,m.id AS image_id,m.width AS image_width,m.height AS image_height \
    FROM catalog_product_images g \
    JOIN media_objects m ON m.key=g.image_key";
impl Products {
    pub async fn gallery(&self, session: &Session, id: i64) -> Result<Vec<GalleryImage>> {
        self.identity
            .authorize(session, Permission::ProductsRead)
            .await?;
        self.gallery_rows(id, false).await
    }
    /// Public callers cannot read a draft's gallery, even by knowing its product ID.
    pub async fn public_gallery(&self, id: i64) -> Result<Vec<GalleryImage>> {
        self.gallery_rows(id, true).await
    }
    async fn gallery_rows(&self, id: i64, public: bool) -> Result<Vec<GalleryImage>> {
        let visibility = if public {
            " JOIN catalog_visible_products p ON p.id=g.product_id"
        } else {
            ""
        };
        let sql = format!(
            "{GALLERY_SELECT}{visibility} WHERE g.product_id=$1 ORDER BY g.position,g.id LIMIT 12"
        );
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .fetch_all(&self.pool)
            .await?)
    }
    async fn gallery_transaction(
        &self,
        session: &Session,
        csrf: &str,
        id: i64,
    ) -> Result<(Transaction<'_, Postgres>, Option<String>)> {
        let mut tx = self.pool.begin().await?;
        MediaStore::publish_lock(&mut tx).await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::ProductsWrite).await?;
        crate::taxes::product_write_lock(&mut tx).await?;
        let primary = sqlx::query_scalar::<_, Option<String>>(
            "SELECT image_key FROM catalog_products WHERE id=$1 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ProductError::NotFound)?;
        Ok((tx, primary))
    }
    pub async fn add_gallery_image(
        &self,
        session: &Session,
        csrf: &str,
        id: i64,
        raw: Vec<u8>,
        filename: &str,
    ) -> Result<()> {
        session.check_csrf(csrf)?;
        self.identity
            .authorize(session, Permission::ProductsWrite)
            .await?;
        let prepared = self.media.prepare_product(raw).await?;
        let (mut tx, _) = self.gallery_transaction(session, csrf, id).await?;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM catalog_product_images WHERE product_id=$1")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        if count >= MAX_GALLERY_IMAGES as i64 {
            return Err(ProductError::GalleryFull);
        }
        let stored = self.media.put_prepared(prepared, filename).await?;
        stored.register(&mut tx, true).await?;
        // Services compact after deletion; max also tolerates gaps made by direct SQL.
        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT id FROM catalog_product_images WHERE product_id=$1 ORDER BY position,id",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        let ordered: Vec<i64> = rows.into_iter().map(|r| r.0).collect();
        reorder(&mut tx, &ordered).await?;
        sqlx::query(
            "INSERT INTO catalog_product_images(product_id,image_key,position) VALUES($1,$2,$3)",
        )
        .bind(id)
        .bind(&stored.key)
        .bind((count + 1) as i32)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn change_gallery_image(
        &self,
        session: &Session,
        csrf: &str,
        id: i64,
        image: i64,
        action: GalleryAction,
    ) -> Result<()> {
        let (mut tx, primary) = self.gallery_transaction(session, csrf, id).await?;
        let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id,image_key FROM catalog_product_images WHERE product_id=$1 ORDER BY position,id")
            .bind(id).fetch_all(&mut *tx).await?;
        let mut ordered: Vec<i64> = rows.iter().map(|r| r.0).collect();
        let index = ordered
            .iter()
            .position(|r| *r == image)
            .ok_or(ProductError::NotFound)?;
        match action {
            GalleryAction::Earlier if index > 0 => ordered.swap(index, index - 1),
            GalleryAction::Later if index + 1 < ordered.len() => ordered.swap(index, index + 1),
            GalleryAction::Primary => {
                sqlx::query("UPDATE catalog_products SET image_key=$1 WHERE id=$2")
                    .bind(&rows[index].1)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                if let Some(key) = primary {
                    sqlx::query("UPDATE catalog_product_images SET image_key=$1 WHERE id=$2")
                        .bind(key)
                        .bind(image)
                        .execute(&mut *tx)
                        .await?;
                } else {
                    remove(&mut tx, image).await?;
                    ordered.remove(index);
                }
            }
            GalleryAction::Remove => {
                remove(&mut tx, image).await?;
                ordered.remove(index);
            }
            _ => {}
        }
        reorder(&mut tx, &ordered).await?;
        tx.commit().await?;
        Ok(())
    }
}
async fn reorder(tx: &mut Transaction<'_, Postgres>, ordered: &[i64]) -> Result<()> {
    sqlx::query(
        "UPDATE catalog_product_images g \
        SET position=o.position::integer \
        FROM unnest($1::bigint[]) WITH ORDINALITY AS o(id,position) \
        WHERE g.id=o.id",
    )
    .bind(ordered)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
async fn remove(tx: &mut Transaction<'_, Postgres>, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM catalog_product_images WHERE id=$1")
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gallery_actions_are_explicit_and_whitelisted() {
        for value in ["earlier", "later", "primary", "remove"] {
            assert!(GalleryAction::parse(value).is_ok());
        }
        for value in ["", "delete", "../primary", "<script>"] {
            assert!(matches!(
                GalleryAction::parse(value),
                Err(ProductError::GalleryAction)
            ));
        }
    }
}
