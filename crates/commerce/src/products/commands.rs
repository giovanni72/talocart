//! Authorized mutations owned by Products.
use super::*;

impl Products {
    pub async fn save(
        &self,
        session: &Session,
        csrf: &str,
        id: Option<i64>,
        input: &Values,
    ) -> Result<i64> {
        self.save_with_content(session, csrf, id, input, None).await
    }
    pub async fn save_with_content(
        &self,
        session: &Session,
        csrf: &str,
        id: Option<i64>,
        input: &Values,
        content: Option<&Draft>,
    ) -> Result<i64> {
        session.check_csrf(csrf)?;
        session.require(Permission::ProductsWrite)?;
        if input
            .related_fallback
            .as_deref()
            .is_some_and(|mode| !["none", "category", "brand", "either"].contains(&mode))
        {
            return Err(ProductError::Invalid(vec![]));
        }
        let mut tx = self.pool.begin().await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::ProductsWrite).await?;
        // The tax owner serializes this check with preference changes and other product writes.
        if !crate::taxes::product_mode_matches(&mut tx, &input.price_mode).await?
            || sqlx::query_scalar::<_, String>("SELECT currency FROM store_settings WHERE id=1")
                .fetch_one(&mut *tx)
                .await?
                != input.currency
        {
            return Err(ProductError::ModeChanged);
        }
        let mut effective = input.clone();
        if let Some(id) = id {
            let (revision, price, stock, configured): (i64, i64, i32, bool) = sqlx::query_as(
                "SELECT p.stock_revision,p.price_cents,p.stock,EXISTS(SELECT 1 FROM catalog_product_options a WHERE a.product_id=p.id) FROM catalog_products p WHERE p.id=$1 FOR UPDATE",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ProductError::NotFound)?;
            if revision != input.stock_revision {
                return Err(ProductError::StockChanged);
            }
            // Configuration is read under the same fiscal lock as variant writes.
            // Ignore even stale or forged parent price/stock fields when variants own them.
            if configured {
                effective.price = format!("{},{:02}", price / 100, price % 100);
            }
            if configured || !input.manage_stock {
                effective.stock = stock.to_string();
            }
        }
        if id.is_none() && !input.manage_stock {
            effective.stock = "0".into();
        }
        let v = validate(&effective)?;
        let definition = crate::content::Resource::Product.definition();
        let translations = definition
            .prepare(
                &mut tx,
                content,
                id.unwrap_or(0),
                Translation {
                    name: v.name.clone(),
                    description: v.description.clone(),
                    seo_title: v.seo_title.clone(),
                    seo_description: v.seo_description.clone(),
                    ..Translation::default()
                },
            )
            .await?;
        let sql = if id.is_some() {
            "UPDATE catalog_products \
                SET name=$1,model=$2,slug=$3,description=$4,price_cents=$5,stock=$6,status=$7,category_id=$8,\
                brand_id=$9,tax_class_id=$10,seo_title=$11,seo_description=$12,minimum_quantity=$13,\
                packs=$14,manage_stock=$15 \
                WHERE id=$16 \
                RETURNING id"
        } else {
            "INSERT INTO catalog_products(name,model,slug,description,price_cents,stock,status,\
                    category_id,brand_id,tax_class_id,seo_title,seo_description,minimum_quantity,packs,manage_stock) \
                VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15) \
                RETURNING id"
        };
        let mut query = sqlx::query_scalar::<_, i64>(sql)
            .bind(v.name)
            .bind(v.model)
            .bind(v.slug)
            .bind(v.description)
            .bind(v.price)
            .bind(v.stock)
            .bind(v.status)
            .bind(v.category)
            .bind(v.brand)
            .bind(v.tax)
            .bind(v.seo_title)
            .bind(v.seo_description)
            .bind(v.minimum_quantity)
            .bind(v.packs)
            .bind(input.manage_stock);
        if let Some(id) = id {
            query = query.bind(id);
        }
        let saved = query
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ProductError::NotFound)?;
        definition.persist(&mut tx, saved, &translations).await?;
        if let Some(mode) = &input.related_fallback {
            sqlx::query("INSERT INTO catalog_related_settings(product_id,fallback) VALUES($1,$2) ON CONFLICT(product_id) DO UPDATE SET fallback=EXCLUDED.fallback")
                .bind(saved).bind(mode).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(saved)
    }
    pub async fn delete(&self, session: &Session, csrf: &str, id: i64) -> Result<()> {
        self.delete_selected(session, csrf, &ProductSelection::new(vec![id])?)
            .await
    }
    pub async fn image(
        &self,
        session: &Session,
        csrf: &str,
        id: i64,
        raw: Option<Vec<u8>>,
        filename: &str,
    ) -> Result<()> {
        session.check_csrf(csrf)?;
        self.identity
            .authorize(session, Permission::ProductsWrite)
            .await?;
        let prepared = match raw {
            Some(raw) => Some(self.media.prepare_product(raw).await?),
            None => None,
        };
        let mut tx = self.pool.begin().await?;
        MediaStore::publish_lock(&mut tx).await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::ProductsWrite).await?;
        crate::taxes::product_write_lock(&mut tx).await?;
        if !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM catalog_products WHERE id=$1)",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?
        {
            return Err(ProductError::NotFound);
        }
        let stored = match prepared {
            Some(p) => Some(self.media.put_prepared(p, filename).await?),
            None => None,
        };
        if let Some(stored) = &stored {
            stored.register(&mut tx, true).await?;
        }
        sqlx::query("UPDATE catalog_products SET image_key=$1 WHERE id=$2")
            .bind(stored.as_ref().map(|s| &s.key))
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    /// Assign an existing public library source, without publishing another copy.
    pub async fn select_image(
        &self,
        session: &Session,
        csrf: &str,
        id: i64,
        media_id: i64,
        gallery: bool,
    ) -> Result<()> {
        if gallery {
            return self
                .select_gallery_images(session, csrf, id, &[media_id])
                .await;
        }
        let mut tx = self.pool.begin().await?;
        MediaStore::publish_lock(&mut tx).await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::ProductsWrite).await?;
        Identity::authorize_transaction(&mut tx, session, csrf, Permission::MediaRead).await?;
        crate::taxes::product_write_lock(&mut tx).await?;
        if sqlx::query_scalar::<_, i64>("SELECT id FROM catalog_products WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .is_none()
        {
            return Err(ProductError::NotFound);
        }
        let key = MediaStore::selected_key(&mut tx, media_id, None).await?;
        if gallery {
            let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(crate::content::query(
                "SELECT count(*) FROM catalog_product_images WHERE product_id=$1",
                self.language,
            )))
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
            if count >= gallery::MAX_GALLERY_IMAGES as i64 {
                return Err(ProductError::GalleryFull);
            }
            let duplicate:bool=sqlx::query_scalar(sqlx::AssertSqlSafe(crate::content::query("SELECT EXISTS(SELECT 1 FROM catalog_product_images WHERE product_id=$1 AND image_key=$2)", self.language))).bind(id).bind(&key).fetch_one(&mut *tx).await?;
            if duplicate {
                return Err(ProductError::Image);
            }
            sqlx::query("INSERT INTO catalog_product_images(product_id,image_key,position) VALUES($1,$2,$3)").bind(id).bind(key).bind(count as i32+1).execute(&mut *tx).await?;
        } else {
            sqlx::query("UPDATE catalog_products SET image_key=$1 WHERE id=$2")
                .bind(key)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    /// A multiple selection is atomic, including capacity, duplicate and stale IDs.
    pub async fn select_gallery_images(
        &self,
        s: &Session,
        csrf: &str,
        id: i64,
        media_ids: &[i64],
    ) -> Result<()> {
        let mut unique = media_ids.to_vec();
        unique.sort_unstable();
        unique.dedup();
        if media_ids.is_empty()
            || media_ids.len() > gallery::MAX_GALLERY_IMAGES
            || unique.len() != media_ids.len()
        {
            return Err(ProductError::Image);
        }
        let mut tx = self.pool.begin().await?;
        MediaStore::publish_lock(&mut tx).await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::ProductsWrite).await?;
        Identity::authorize_transaction(&mut tx, s, csrf, Permission::MediaRead).await?;
        crate::taxes::product_write_lock(&mut tx).await?;
        if sqlx::query_scalar::<_, i64>("SELECT id FROM catalog_products WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .is_none()
        {
            return Err(ProductError::NotFound);
        }
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(crate::content::query(
            "SELECT count(*) FROM catalog_product_images WHERE product_id=$1",
            self.language,
        )))
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if count + media_ids.len() as i64 > gallery::MAX_GALLERY_IMAGES as i64 {
            return Err(ProductError::GalleryFull);
        }
        let mut keys = Vec::new();
        for media_id in media_ids {
            let key = MediaStore::selected_key(&mut tx, *media_id, None).await?;
            if sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM catalog_product_images WHERE product_id=$1 AND image_key=$2)").bind(id).bind(&key).fetch_one(&mut *tx).await?{return Err(ProductError::Image);}
            keys.push(key);
        }
        for (offset, key) in keys.iter().enumerate() {
            sqlx::query("INSERT INTO catalog_product_images(product_id,image_key,position) VALUES($1,$2,$3)").bind(id).bind(key).bind((count+offset as i64+1) as i32).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    /// Deduplicate direct categories before walking ancestors, independently of
    /// product count. UNION keeps shared ancestors unique; only active choices appear.
    pub(super) async fn public_relevant_categories(
        &self,
        brand: &str,
    ) -> Result<Vec<(String, String)>> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(crate::content::query(
            "WITH RECURSIVE relevant_categories(id,parent_id) AS (SELECT c.id,c.parent_id \
                    FROM catalog_categories c \
                    WHERE EXISTS (SELECT 1 \
                        FROM [[visible_products]] p \
                        WHERE p.category_id=c.id \
                        AND ($1='' \
                            OR EXISTS (SELECT 1 \
                                FROM catalog_brands b \
                                WHERE b.id=p.brand_id \
                                AND b.slug=$1))) \
                    UNION \
                    SELECT c.id,c.parent_id \
                    FROM catalog_categories c \
                    JOIN relevant_categories child ON child.parent_id=c.id) \
                SELECT c.name,c.slug \
                FROM [[categories]] c \
                WHERE c.active \
                AND c.id IN (SELECT id \
                    FROM relevant_categories) \
                ORDER BY lower(c.name),c.id \
                LIMIT 1000",
            self.language,
        )))
        .bind(brand)
        .fetch_all(&self.pool)
        .await?)
    }
}
