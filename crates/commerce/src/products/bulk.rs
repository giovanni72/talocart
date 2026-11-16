use super::*;

/// A bounded set of distinct positive IDs. Sorting also fixes row-lock ordering.
#[derive(Debug)]
pub struct ProductSelection(Vec<i64>);
impl ProductSelection {
    /// Bulk actions cover the largest product listing page, without unbounded batches.
    pub const MAX_LEN: usize = 500;
    pub fn new(mut ids: Vec<i64>) -> Result<Self> {
        if ids.is_empty() || ids.len() > Self::MAX_LEN || ids.iter().any(|id| *id <= 0) {
            return Err(ProductError::Selection);
        }
        ids.sort_unstable();
        if ids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(ProductError::Selection);
        }
        Ok(Self(ids))
    }
}

/// Only explicit catalog actions can enter a bulk mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BulkAction {
    Delete,
    Publish,
    Draft,
    AssignCategory,
}
impl BulkAction {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "delete" => Ok(Self::Delete),
            "publish" => Ok(Self::Publish),
            "draft" => Ok(Self::Draft),
            "category" => Ok(Self::AssignCategory),
            _ => Err(ProductError::BulkAction),
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Delete => "delete",
            Self::Publish => "publish",
            Self::Draft => "draft",
            Self::AssignCategory => "category",
        }
    }
}

/// Validated compound plan: one optional state, one optional category, or deletion alone.
#[derive(Debug)]
pub struct BulkActions {
    delete: bool,
    status: Option<&'static str>,
    category: Option<i64>,
}
impl BulkActions {
    pub fn new(actions: &[BulkAction], category: Option<i64>) -> Result<Self> {
        if actions.is_empty()
            || actions.len() > 4
            || actions
                .iter()
                .enumerate()
                .any(|(index, action)| actions[..index].contains(action))
            || (actions.contains(&BulkAction::Delete) && actions.len() != 1)
            || (actions.contains(&BulkAction::Publish) && actions.contains(&BulkAction::Draft))
        {
            return Err(ProductError::BulkAction);
        }
        let category = if actions.contains(&BulkAction::AssignCategory) {
            Some(
                category
                    .filter(|id| *id > 0)
                    .ok_or(ProductError::Invalid(vec![Field::Category]))?,
            )
        } else {
            None
        };
        Ok(Self {
            delete: actions.contains(&BulkAction::Delete),
            status: if actions.contains(&BulkAction::Publish) {
                Some("published")
            } else if actions.contains(&BulkAction::Draft) {
                Some("draft")
            } else {
                None
            },
            category,
        })
    }
    pub fn deletes(&self) -> bool {
        self.delete
    }
    pub fn status(&self) -> Option<&'static str> {
        self.status
    }
    pub fn category(&self) -> Option<i64> {
        self.category
    }
    pub fn combined(&self) -> bool {
        self.status.is_some() && self.category.is_some()
    }
}

impl Products {
    pub async fn selected(
        &self,
        session: &Session,
        selection: &ProductSelection,
    ) -> Result<Vec<i64>> {
        self.identity
            .authorize(session, Permission::ProductsWrite)
            .await?;
        self.identity
            .authorize(session, Permission::ProductsRead)
            .await?;
        let rows = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM catalog_products WHERE id=ANY($1) ORDER BY id",
        )
        .bind(&selection.0)
        .fetch_all(&self.pool)
        .await?;
        if rows.len() != selection.0.len() {
            return Err(ProductError::NotFound);
        }
        Ok(rows)
    }

    /// Deletes the whole selection or nothing. References and media lifecycle match single delete.
    pub async fn delete_selected(
        &self,
        session: &Session,
        csrf: &str,
        selection: &ProductSelection,
    ) -> Result<()> {
        self.apply_actions_selected(
            session,
            csrf,
            selection,
            &BulkActions::new(&[BulkAction::Delete], None)?,
        )
        .await
    }

    pub async fn assign_category_selected(
        &self,
        session: &Session,
        csrf: &str,
        selection: &ProductSelection,
        category: i64,
    ) -> Result<()> {
        self.identity
            .authorize(session, Permission::ProductsWrite)
            .await?;
        session.check_csrf(csrf)?;
        self.apply_actions_selected(
            session,
            csrf,
            selection,
            &BulkActions::new(&[BulkAction::AssignCategory], Some(category))?,
        )
        .await
    }

    /// Category and status change together in one UPDATE; publication sees the final values.
    pub async fn apply_actions_selected(
        &self,
        session: &Session,
        csrf: &str,
        selection: &ProductSelection,
        actions: &BulkActions,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        if actions.delete {
            MediaStore::publish_lock(&mut tx).await?;
        }
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::ProductsWrite).await?;
        crate::taxes::product_write_lock(&mut tx).await?;
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM catalog_products WHERE id=ANY($1) ORDER BY id FOR UPDATE",
        )
        .bind(&selection.0)
        .fetch_all(&mut *tx)
        .await?;
        if ids != selection.0 {
            return Err(ProductError::NotFound);
        }
        if let Some(category) = actions.category
            && !crate::product_category_exists(&mut tx, category).await?
        {
            return Err(ProductError::Invalid(vec![Field::Category]));
        }
        if actions.delete {
            sqlx::query("DELETE FROM catalog_products WHERE id=ANY($1)")
                .bind(&selection.0)
                .execute(&mut *tx)
                .await
                .map_err(|error| {
                    if error
                        .as_database_error()
                        .is_some_and(|db| db.is_foreign_key_violation())
                    {
                        ProductError::InUse
                    } else {
                        error.into()
                    }
                })?;
        } else {
            // The existing guard checks publication against both final fields, so publishing
            // and repairing/assigning a category is atomic even if the old reference was inactive.
            sqlx::query(
                "UPDATE catalog_products \
                SET status=coalesce($2::text,status),category_id=coalesce($3::bigint,category_id) \
                WHERE id=ANY($1)",
            )
            .bind(&selection.0)
            .bind(actions.status)
            .bind(actions.category)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compound_actions_reject_conflicts_duplicates_and_missing_category() {
        for actions in [
            vec![],
            vec![BulkAction::Delete, BulkAction::Publish],
            vec![BulkAction::Publish, BulkAction::Draft],
            vec![BulkAction::Draft, BulkAction::Draft],
        ] {
            assert!(matches!(
                BulkActions::new(&actions, Some(1)),
                Err(ProductError::BulkAction)
            ));
        }
        assert!(BulkActions::new(&[BulkAction::AssignCategory], None).is_err());
        let plan =
            BulkActions::new(&[BulkAction::Publish, BulkAction::AssignCategory], Some(3)).unwrap();
        assert_eq!(plan.status(), Some("published"));
        assert_eq!(plan.category(), Some(3));
        assert!(plan.combined());
        assert_eq!(
            BulkActions::new(&[BulkAction::Publish], Some(999))
                .unwrap()
                .category(),
            None
        );
    }
    #[test]
    fn actions_are_explicit_and_allowlisted() {
        for (key, action) in [
            ("delete", BulkAction::Delete),
            ("publish", BulkAction::Publish),
            ("draft", BulkAction::Draft),
            ("category", BulkAction::AssignCategory),
        ] {
            assert_eq!(BulkAction::parse(key).unwrap(), action);
            assert_eq!(action.key(), key);
        }
        for key in [
            "",
            "disabled",
            "published",
            "DELETE",
            "delete; DROP TABLE catalog_products",
        ] {
            assert!(matches!(
                BulkAction::parse(key),
                Err(ProductError::BulkAction)
            ));
        }
    }
    #[test]
    fn selection_is_bounded_distinct_positive_and_sorted() {
        for ids in [vec![], vec![0], vec![-1], vec![1, 1], (1..=501).collect()] {
            assert!(matches!(
                ProductSelection::new(ids),
                Err(ProductError::Selection)
            ));
        }
        assert_eq!(ProductSelection::new(vec![3, 1, 2]).unwrap().0, [1, 2, 3]);
        for count in [100, 101, 500] {
            assert!(ProductSelection::new((1..=count).collect()).is_ok());
        }
        assert_eq!(
            ProductSelection::MAX_LEN,
            usize::from(*PRODUCT_PAGE_SIZES.last().unwrap())
        );
    }
}
