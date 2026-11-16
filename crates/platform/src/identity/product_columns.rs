//! Personal product-table preferences. The caller can only change their own account.
use super::*;

pub const PRODUCT_COLUMNS: [&str; 14] = [
    "selection",
    "id",
    "image",
    "type",
    "name",
    "model",
    "category",
    "brand",
    "price",
    "variants",
    "stock",
    "status",
    "modified_at",
    "actions",
];

pub struct ProductColumns {
    hidden: Vec<String>,
}
impl ProductColumns {
    /// Actions always remain visible, even when no optional columns are selected.
    pub fn from_visible(visible: &[String]) -> Result<Self> {
        if visible.len() > PRODUCT_COLUMNS.len()
            || visible
                .iter()
                .any(|key| !PRODUCT_COLUMNS.contains(&key.as_str()))
        {
            return Err(IdentityError::Invalid);
        }
        Ok(Self {
            hidden: PRODUCT_COLUMNS
                .iter()
                .filter(|key| **key != "actions" && !visible.iter().any(|v| v == **key))
                .map(|key| (*key).to_owned())
                .collect(),
        })
    }
    pub fn visible(&self, key: &str) -> bool {
        key == "actions" || !self.hidden.iter().any(|hidden| hidden == key)
    }
    pub fn count(&self) -> usize {
        PRODUCT_COLUMNS
            .iter()
            .filter(|key| self.visible(key))
            .count()
    }
}
impl Identity {
    pub async fn product_columns(&self, session: &Session) -> Result<ProductColumns> {
        self.authorize(session, Permission::ProductsRead).await?;
        let hidden =
            sqlx::query_scalar("SELECT product_hidden_columns FROM administrators WHERE id=$1")
                .bind(session.administrator_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(ProductColumns { hidden })
    }
    pub async fn save_product_columns(
        &self,
        session: &Session,
        csrf: &str,
        columns: &ProductColumns,
    ) -> Result<()> {
        session.check_csrf(csrf)?;
        let mut tx = self.pool.begin().await?;
        Self::lock_changes(&mut tx).await?;
        let actor = Self::authorize_locked(&mut tx, session, Permission::ProductsRead).await?;
        sqlx::query("UPDATE administrators SET product_hidden_columns=$1 WHERE id=$2")
            .bind(&columns.hidden)
            .bind(actor.administrator_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_and_canonicalizes_product_columns() {
        let actions = ProductColumns::from_visible(&[]).expect("actions only");
        assert_eq!(actions.count(), 1);
        assert!(actions.visible("actions"));
        assert!(ProductColumns::from_visible(&["unknown".into()]).is_err());
        let all =
            ProductColumns::from_visible(&PRODUCT_COLUMNS.map(str::to_owned)).expect("all columns");
        assert_eq!(all.count(), 14);
        let one = ProductColumns::from_visible(&["name".into(), "name".into()])
            .expect("one visible column");
        assert_eq!(one.count(), 2);
        assert!(one.visible("name"));
        assert!(!one.visible("id"));
    }
}
