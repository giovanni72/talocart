//! Authorized mutations owned by Variants.
use super::*;

impl Variants {
    pub(super) async fn write<'a>(
        &'a self,
        s: &Session,
        csrf: &str,
    ) -> Result<Transaction<'a, Postgres>> {
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::ProductsWrite).await?;
        crate::taxes::product_write_lock(&mut tx).await?;
        Ok(tx)
    }
    pub async fn save_option(
        &self,
        s: &Session,
        csrf: &str,
        id: Option<i64>,
        input: &str,
    ) -> Result<i64> {
        self.save_option_with_content(s, csrf, id, input, None)
            .await
    }
    pub async fn save_option_with_content(
        &self,
        s: &Session,
        csrf: &str,
        id: Option<i64>,
        input: &str,
        content: Option<&Draft>,
    ) -> Result<i64> {
        self.save_option_details(
            s,
            csrf,
            id,
            &OptionInput {
                name: input,
                sort_order: None,
            },
            content,
        )
        .await
    }
    pub async fn save_option_details(
        &self,
        s: &Session,
        csrf: &str,
        id: Option<i64>,
        input: &OptionInput<'_>,
        content: Option<&Draft>,
    ) -> Result<i64> {
        s.check_csrf(csrf)?;
        s.require(Permission::ProductsWrite)?;
        let name = name(input.name)?;
        let sort_order = sort_order(input.sort_order)?;
        let mut tx = self.write(s, csrf).await?;
        let definition = crate::content::Resource::Option.definition();
        let translations = definition
            .prepare(
                &mut tx,
                content,
                id.unwrap_or(0),
                Translation {
                    name: name.clone(),
                    ..Translation::default()
                },
            )
            .await?;
        let sql = if id.is_some() {
            "UPDATE catalog_options SET name=$1,sort_order=coalesce($2,sort_order) WHERE id=$3 RETURNING id"
        } else {
            "INSERT INTO catalog_options(name,sort_order) VALUES($1,coalesce($2,0)) RETURNING id"
        };
        let mut q = sqlx::query_scalar(sql).bind(name).bind(sort_order);
        if let Some(id) = id {
            q = q.bind(id);
        }
        let id = q
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(VariantError::NotFound)?;
        definition.persist(&mut tx, id, &translations).await?;
        tx.commit().await?;
        Ok(id)
    }
    pub async fn delete_option(&self, s: &Session, csrf: &str, id: i64) -> Result<()> {
        let mut tx = self.write(s, csrf).await?;
        sqlx::query("DELETE FROM catalog_option_values WHERE option_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if sqlx::query("DELETE FROM catalog_options WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            == 0
        {
            return Err(VariantError::NotFound);
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn save_value(
        &self,
        s: &Session,
        csrf: &str,
        option: i64,
        id: Option<i64>,
        input: &str,
    ) -> Result<i64> {
        self.save_value_with_content(s, csrf, option, id, input, None)
            .await
    }
    pub async fn save_value_with_content(
        &self,
        s: &Session,
        csrf: &str,
        option: i64,
        id: Option<i64>,
        input: &str,
        content: Option<&Draft>,
    ) -> Result<i64> {
        self.save_value_details(
            s,
            csrf,
            option,
            id,
            &OptionInput {
                name: input,
                sort_order: None,
            },
            content,
        )
        .await
    }
    pub async fn save_value_details(
        &self,
        s: &Session,
        csrf: &str,
        option: i64,
        id: Option<i64>,
        input: &OptionInput<'_>,
        content: Option<&Draft>,
    ) -> Result<i64> {
        s.check_csrf(csrf)?;
        s.require(Permission::ProductsWrite)?;
        let name = name(input.name)?;
        let sort_order = sort_order(input.sort_order)?;
        self.option(s, option).await?;
        let mut tx = self.write(s, csrf).await?;
        let definition = crate::content::Resource::Value.definition();
        let translations = definition
            .prepare(
                &mut tx,
                content,
                id.unwrap_or(0),
                Translation {
                    name: name.clone(),
                    ..Translation::default()
                },
            )
            .await?;
        let sql = if id.is_some() {
            "UPDATE catalog_option_values \
                SET name=$1,sort_order=coalesce($3,sort_order) \
                WHERE option_id=$2 \
                AND id=$4 \
                RETURNING id"
        } else {
            "INSERT INTO catalog_option_values(name,option_id,sort_order) \
                VALUES($1,$2,coalesce($3,0)) \
                RETURNING id"
        };
        let mut q = sqlx::query_scalar(sql)
            .bind(name)
            .bind(option)
            .bind(sort_order);
        if let Some(id) = id {
            q = q.bind(id);
        }
        let id = q
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(VariantError::NotFound)?;
        definition.persist(&mut tx, id, &translations).await?;
        tx.commit().await?;
        Ok(id)
    }
    pub async fn delete_value(&self, s: &Session, csrf: &str, option: i64, id: i64) -> Result<()> {
        let mut tx = self.write(s, csrf).await?;
        if sqlx::query("DELETE FROM catalog_option_values WHERE option_id=$1 AND id=$2")
            .bind(option)
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            == 0
        {
            return Err(VariantError::NotFound);
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn configure(
        &self,
        s: &Session,
        csrf: &str,
        product: i64,
        options: &[i64],
    ) -> Result<()> {
        s.check_csrf(csrf)?;
        s.require(Permission::ProductsWrite)?;
        let mut sorted = options.to_vec();
        sorted.sort_unstable();
        if options.len() > 3
            || options.iter().any(|n| *n <= 0)
            || sorted.windows(2).any(|w| w[0] == w[1])
        {
            return Err(VariantError::Options);
        }
        let mut tx = self.write(s, csrf).await?;
        if !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM catalog_products WHERE id=$1)",
        )
        .bind(product)
        .fetch_one(&mut *tx)
        .await?
        {
            return Err(VariantError::NotFound);
        }
        if sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM catalog_variants WHERE product_id=$1)",
        )
        .bind(product)
        .fetch_one(&mut *tx)
        .await?
        {
            return Err(VariantError::InUse);
        }
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(crate::content::query(
            "SELECT count(*) FROM [[options]] WHERE id=ANY($1)",
            self.language,
        )))
        .bind(options)
        .fetch_one(&mut *tx)
        .await?;
        if count != options.len() as i64 {
            return Err(VariantError::Options);
        }
        // Removing the configuration must not immediately sell conserved parent values.
        // Only an actual configured-to-simple transition changes publication status.
        if options.is_empty() {
            sqlx::query("UPDATE catalog_products SET status='draft' WHERE id=$1 AND EXISTS(SELECT 1 FROM catalog_product_options WHERE product_id=$1)")
                .bind(product).execute(&mut *tx).await?;
        }
        sqlx::query("DELETE FROM catalog_product_options WHERE product_id=$1")
            .bind(product)
            .execute(&mut *tx)
            .await?;
        for (n, id) in options.iter().enumerate() {
            sqlx::query("INSERT INTO catalog_product_options(product_id,position,option_id) VALUES($1,$2,$3)").bind(product).bind((n+1)as i16).bind(id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn save(
        &self,
        s: &Session,
        csrf: &str,
        product: i64,
        id: Option<i64>,
        v: &Values,
    ) -> Result<i64> {
        s.check_csrf(csrf)?;
        s.require(Permission::ProductsWrite)?;
        let mut tx = self.write(s, csrf).await?;
        if !crate::taxes::product_mode_matches(&mut tx, &v.price_mode).await? {
            return Err(ProductError::ModeChanged.into());
        }
        let manage_stock: bool =
            sqlx::query_scalar("SELECT manage_stock FROM catalog_products WHERE id=$1 FOR UPDATE")
                .bind(product)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(VariantError::NotFound)?;
        let mut effective = v.clone();
        if let Some(id) = id {
            let (revision, stock): (i64, i32) = sqlx::query_as("SELECT stock_revision,stock FROM catalog_variants WHERE id=$1 AND product_id=$2 FOR UPDATE")
                .bind(id).bind(product).fetch_optional(&mut *tx).await?.ok_or(VariantError::NotFound)?;
            if revision != v.stock_revision {
                return Err(ProductError::StockChanged.into());
            }
            if !manage_stock {
                effective.stock = stock.to_string();
            }
        } else if !manage_stock {
            effective.stock = "0".into();
        }
        let (sku, price, stock, ids) = validate(&effective)?;
        let sql = if id.is_some() {
            "UPDATE catalog_variants \
                SET sku=$1,price_cents=$2,stock=$3,active=$4,value_ids=$5 \
                WHERE product_id=$6 \
                AND id=$7 \
                RETURNING id"
        } else {
            "INSERT INTO catalog_variants(sku,price_cents,stock,active,value_ids,product_id) \
                VALUES($1,$2,$3,$4,$5,$6) \
                RETURNING id"
        };
        let mut q = sqlx::query_scalar(sql)
            .bind(sku)
            .bind(price)
            .bind(stock)
            .bind(v.active)
            .bind(ids)
            .bind(product);
        if let Some(id) = id {
            q = q.bind(id);
        }
        let id = q
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(VariantError::NotFound)?;
        tx.commit().await?;
        Ok(id)
    }
    pub async fn delete(&self, s: &Session, csrf: &str, product: i64, id: i64) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        MediaStore::publish_lock(&mut tx).await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::ProductsWrite).await?;
        if sqlx::query("DELETE FROM catalog_variants WHERE product_id=$1 AND id=$2")
            .bind(product)
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            == 0
        {
            return Err(VariantError::NotFound);
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn select_image(
        &self,
        s: &Session,
        csrf: &str,
        product: i64,
        id: i64,
        media_id: i64,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        MediaStore::publish_lock(&mut tx).await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::ProductsWrite).await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::MediaRead).await?;
        let key = MediaStore::selected_key(&mut tx, media_id, None).await?;
        let result =
            sqlx::query("UPDATE catalog_variants SET image_key=$1 WHERE id=$2 AND product_id=$3")
                .bind(key)
                .bind(id)
                .bind(product)
                .execute(&mut *tx)
                .await?;
        if result.rows_affected() != 1 {
            return Err(VariantError::NotFound);
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn image(
        &self,
        s: &Session,
        csrf: &str,
        product: i64,
        id: i64,
        raw: Option<Vec<u8>>,
        filename: &str,
    ) -> Result<()> {
        s.check_csrf(csrf)?;
        self.identity
            .authorize(s, Permission::ProductsWrite)
            .await?;
        let prepared = match raw {
            Some(raw) => Some(self.media.prepare_product(raw).await?),
            None => None,
        };
        let mut tx = self.pool.begin().await?;
        MediaStore::publish_lock(&mut tx).await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::ProductsWrite).await?;
        if !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM catalog_variants WHERE product_id=$1 AND id=$2)",
        )
        .bind(product)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?
        {
            return Err(VariantError::NotFound);
        }
        let stored = match prepared {
            Some(p) => Some(self.media.put_prepared(p, filename).await?),
            None => None,
        };
        if let Some(stored) = &stored {
            stored.register(&mut tx, true).await?;
        }
        sqlx::query("UPDATE catalog_variants SET image_key=$1 WHERE product_id=$2 AND id=$3")
            .bind(stored.as_ref().map(|s| &s.key))
            .bind(product)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
